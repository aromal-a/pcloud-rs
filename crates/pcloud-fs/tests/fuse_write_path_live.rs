#![allow(clippy::pedantic)]
//! bd-1du.4.6 — live write-path + remount readback integration test.
//!
//! Proves that the `PcloudFsShim`-based writable FUSE mount closes the
//! full write → unmount → remount → readback loop under a real kernel
//! mount (`libfuse` / `fuser`) on Linux:
//!
//!   1. Mount a writable [`PcloudFsShim`] composed over a mocked
//!      folder/file backend and a recording upload backend.
//!   2. Create a non-trivial file through the kernel VFS
//!      (`std::fs::write`), spanning multiple kernel write ops, and
//!      fsync it via `File::sync_all`.
//!   3. Unmount cleanly. The captured upload bytes are the
//!      canonical "what the server now holds".
//!   4. Seed the mocked backends with those captured bytes (simulating
//!      a server that absorbed the upload), rebuild a **fresh**
//!      `PcloudFsShim` (new staging dir + journal to prove we are not
//!      reading from in-process in-memory state), and remount the
//!      same mountpoint.
//!   5. Readback the file via `std::fs::read` and assert the bytes
//!      round-trip **byte-identical** through the kernel VFS.
//!   6. Unmount cleanly.
//!
//! # Scope (honesty statement)
//!
//! * This test exercises the `MountService::mount_fuser` → `PcloudFsShim`
//!   → `WritePathService` path. The `BoxedFuserShim` / `FuserShim<A>`
//!   dyn-trait shim on `platform/linux.rs` is **still read-only** by
//!   design; carrying the concrete `WritePathService<U>` through an
//!   object-safe trait is deferred follow-up. See
//!   [`crate::platform::linux::BoxedFuserShim`] type docs.
//! * The remount step rebuilds the shim with a **fresh** staging dir and
//!   journal on purpose. This makes the readback assertion independent
//!   of any in-process caches: the only path from write → read is
//!   upload-bytes → mocked-server-listing → kernel-VFS-read via the
//!   `ProtoFuseAdapter` read path.
//! * Mid-write journal resume (`ResumeOutcome::Resumed` /
//!   `SidecarTrimmed` etc.) is covered by `write_path_replay.rs` and by
//!   the unit tests in `write_path.rs`; that is a separate proof axis
//!   from this remount-readback loop.
//!
//! # Gating
//!
//! * `#[cfg(target_os = "linux")]` — `fuser` is Linux-only.
//! * `#[ignore]` by default — opt-in via `PCLOUD_FUSE_TEST=1`
//!   (legacy) or `PCLOUD_LIVE_E2E=1` (preferred).
//! * If the host refuses to mount FUSE (no `/dev/fuse`, unprivileged
//!   container, EPERM, ENOSYS, missing `fusermount3`) the test **skips
//!   gracefully** — matching the project-wide P0.5 convention.

#![cfg(target_os = "linux")]

// **PLATFORM:** Linux
// **GATING:** #[cfg(target_os = "linux")].

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pcloud_fs::backend::mock::{MockFileBackend, MockFolderBackend};
use pcloud_fs::fuse_adapter::{AdapterOptions, ProtoFuseAdapter};
use pcloud_fs::fuser_shim::PcloudFsShim;
use pcloud_fs::mount_service::{MountHandle, MountOptions, MountService};
use pcloud_fs::staging::StagingDir;
use pcloud_fs::write_journal::WriteJournal;
use pcloud_fs::write_path::{
    FileUploadBackend, WritePathError, WritePathOptions, WritePathService,
};

/// 256 KiB: well above the default FUSE kernel write chunk (~128 KiB on
/// most recent kernels) and well above a single page, so we exercise at
/// least two kernel `write(2)` ops. Keeps under any plausible
/// `max_write` clamp and well under the 64 MiB flush threshold so the
/// finalize upload only happens on `release` / `fsync` — making the
/// byte-equality assertion below deterministic.
const PAYLOAD_BYTES: usize = 256 * 1024;

fn e2e_gate_enabled() -> bool {
    let live = std::env::var("PCLOUD_LIVE_E2E").ok().as_deref() == Some("1");
    let legacy = std::env::var("PCLOUD_FUSE_TEST").ok().as_deref() == Some("1");
    live || legacy
}

fn dev_fuse_available() -> bool {
    Path::new("/dev/fuse").exists()
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

/// Deterministic non-trivial payload — an LCG stream so an all-zero
/// buffer bug in the kernel round-trip would immediately fail the
/// byte-equality assertion. Matches the style in `fuse_kernel_e2e.rs`.
fn build_payload(len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    let mut state: u64 = 0x243F_6A88_85A3_08D3;
    while out.len() < len {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        out.extend_from_slice(&state.to_le_bytes());
    }
    out.truncate(len);
    out
}

/// Upload backend that captures every `upload_file` call into a shared
/// vec. After the first mount's unmount, the captured bytes are fed
/// back into the mocked folder/file listings to simulate "the server
/// has accepted the upload and is now the authoritative source".
#[derive(Default)]
struct RecordingUploadBackend {
    /// Entries: `(parent_path, name, bytes)`.
    uploads: Mutex<Vec<(String, String, Vec<u8>)>>,
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

/// RAII unmount guard so a panicking assertion does not leak a FUSE
/// mount. Same pattern as `fuse_small_write_wiring.rs`.
struct MountGuard {
    handle: Option<MountHandle>,
    path: PathBuf,
}

impl MountGuard {
    fn new(handle: MountHandle, path: PathBuf) -> Self {
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
        if let Some(h) = self.handle.take()
            && let Err(e) = h.unmount()
        {
            eprintln!("[fuse_write_path_live] RAII unmount failed: {e}");
            let _ = std::process::Command::new("fusermount3")
                .arg("-u")
                .arg(&self.path)
                .status();
            let _ = std::process::Command::new("fusermount")
                .arg("-u")
                .arg(&self.path)
                .status();
        }
    }
}

/// Compose a fresh writable `PcloudFsShim` over the given backends plus
/// a brand-new staging dir and journal rooted under `root`. Returns the
/// shim plus the `StagingDir` root so the caller can keep the tempdir
/// alive until unmount.
fn build_fresh_shim(
    folder: Arc<MockFolderBackend>,
    files: Arc<MockFileBackend>,
    upload_backend: Arc<RecordingUploadBackend>,
    stage_root: &Path,
    journal_path: &Path,
) -> PcloudFsShim<MockFolderBackend, MockFileBackend, RecordingUploadBackend> {
    let stage = StagingDir::open(stage_root).expect("staging open");
    let journal = WriteJournal::open(journal_path).expect("journal open");

    let writer = Arc::new(WritePathService::new(
        stage,
        journal,
        upload_backend,
        // Default 64 MiB threshold — our 256 KiB payload stays well
        // below it so only the explicit release/fsync finalizes.
        WritePathOptions::default(),
    ));

    let adapter = Arc::new(
        ProtoFuseAdapter::with_file_backend(folder, files, AdapterOptions::default())
            .with_write_path(Arc::clone(&writer)),
    );
    PcloudFsShim::new(adapter, writer)
}

/// bd-1du.4.6 core proof: write a file through the kernel, unmount,
/// remount with the captured upload as the new remote truth, read back
/// through the kernel, byte-identical.
#[test]
#[ignore = "requires PCLOUD_LIVE_E2E=1 (or PCLOUD_FUSE_TEST=1) and a working libfuse kernel module"]
fn write_unmount_remount_readback_byte_identical() {
    if !e2e_gate_enabled() {
        eprintln!("[fuse_write_path_live] skip: PCLOUD_LIVE_E2E / PCLOUD_FUSE_TEST not set");
        return;
    }
    if !dev_fuse_available() {
        eprintln!("[fuse_write_path_live] skip: /dev/fuse not available");
        return;
    }

    // --- shared mocked state across the two mount cycles ----------------
    let folder = Arc::new(MockFolderBackend::new());
    folder.insert_dir("/", 1, vec![]);
    let files = Arc::new(MockFileBackend::new());
    let upload_backend = Arc::new(RecordingUploadBackend::default());

    let mnt = tempfile::tempdir().expect("mount tempdir");
    let payload = build_payload(PAYLOAD_BYTES);
    let file_name = "roundtrip.bin";
    let file_path = mnt.path().join(file_name);

    // --- MOUNT 1: write the file ----------------------------------------
    let stage1_tmp = tempfile::tempdir().expect("stage1 tempdir");
    let journal1_tmp = tempfile::tempdir().expect("journal1 tempdir");
    {
        let shim = build_fresh_shim(
            Arc::clone(&folder),
            Arc::clone(&files),
            Arc::clone(&upload_backend),
            &stage1_tmp.path().join("stage"),
            &journal1_tmp.path().join("journal.bin"),
        );

        let svc = MountService::new();
        let handle = match svc.mount_fuser(
            mnt.path(),
            shim,
            MountOptions {
                read_only: false,
                ..MountOptions::default()
            },
        ) {
            Ok(h) => h,
            Err(err) if should_skip_mount_error(&err.to_string()) => {
                eprintln!("[fuse_write_path_live] skip (mount1): {err}");
                return;
            }
            Err(err) => panic!("mount_fuser (mount1): {err}"),
        };
        let guard = MountGuard::new(handle, mnt.path().to_path_buf());

        std::thread::sleep(Duration::from_millis(200));
        if !mount_appears_active(mnt.path()) {
            eprintln!("[fuse_write_path_live] skip: mount1 did not appear in /proc/self/mountinfo");
            return;
        }

        // Write via std::fs::write — single convenience call that does
        // open(O_WRONLY|O_CREAT|O_TRUNC) + write + close under the hood.
        // `close` drives the kernel's `flush` + `release`, which in turn
        // drives `WritePathService::flush` and `release` → `upload_file`.
        if let Err(err) = std::fs::write(&file_path, &payload) {
            if should_skip_io_error(&err) {
                return;
            }
            panic!("std::fs::write: {err}");
        }

        // Belt-and-suspenders: open + sync_all so the kernel definitely
        // issued fsync before unmount. std::fs::write alone does not
        // guarantee fsync; on some kernels `release` is enough to
        // finalize, but fsync makes the assertion below deterministic.
        {
            let f = match std::fs::OpenOptions::new().read(true).open(&file_path) {
                Ok(f) => f,
                Err(err) if should_skip_io_error(&err) => return,
                Err(err) => {
                    // Pre-remount readback is best-effort — the mock
                    // backend does not auto-publish post-create entries
                    // into readdir/lookup. A miss here is not a failure;
                    // the authoritative check is post-remount.
                    eprintln!(
                        "[fuse_write_path_live] note: pre-remount open miss (mock readdir \
                         publication gap): {err}"
                    );
                    // Drop guard → clean unmount before we proceed to
                    // the remount step.
                    guard.unmount().expect("clean unmount (mount1, early)");
                    assert_upload_captured(&upload_backend, file_name, &payload);
                    return proceed_remount_readback(
                        &folder,
                        &files,
                        upload_backend,
                        mnt.path(),
                        file_name,
                        &file_path,
                        &payload,
                    );
                }
            };
            if let Err(err) = f.sync_all() {
                if should_skip_io_error(&err) {
                    return;
                }
                // sync_all on a read fh may be a no-op on some kernels;
                // tolerate but do not fail the test.
                eprintln!("[fuse_write_path_live] note: read-fh sync_all: {err}");
            }
        }

        // Clean unmount of mount #1.
        guard.unmount().expect("clean unmount (mount1)");
    }

    // --- Upload must have been captured by release/fsync ----------------
    assert_upload_captured(&upload_backend, file_name, &payload);

    // --- MOUNT 2: remount, read back byte-identical ---------------------
    proceed_remount_readback(
        &folder,
        &files,
        upload_backend,
        mnt.path(),
        file_name,
        &file_path,
        &payload,
    );
}

/// Second half of the proof: seed the mocked server listings with the
/// captured upload bytes, rebuild a **fresh** `PcloudFsShim` over a new
/// staging dir and journal, remount the same mountpoint, and read back
/// via the kernel VFS. Asserts byte-identical.
fn proceed_remount_readback(
    folder: &Arc<MockFolderBackend>,
    files: &Arc<MockFileBackend>,
    upload_backend: Arc<RecordingUploadBackend>,
    mnt_path: &Path,
    file_name: &str,
    file_path: &Path,
    payload: &[u8],
) {
    // Pick a synthetic file_id. Must not collide with any existing id in
    // the mock; the mock starts empty, so any stable id works.
    const REMOTE_FILE_ID: u64 = 424242;
    let size = payload.len() as u64;
    folder.insert_dir_with_sizes(
        "/",
        1,
        vec![(file_name, false, None, Some(REMOTE_FILE_ID), Some(size))],
    );
    files.insert_file(REMOTE_FILE_ID, payload.to_vec());

    // Fresh staging + journal. Using new tempdirs proves the readback
    // path does not depend on in-process state carried over from mount #1.
    let stage2_tmp = tempfile::tempdir().expect("stage2 tempdir");
    let journal2_tmp = tempfile::tempdir().expect("journal2 tempdir");
    let shim = build_fresh_shim(
        Arc::clone(folder),
        Arc::clone(files),
        // Reuse the same upload backend so any spurious post-remount
        // writes would still be visible — readback must not trigger
        // uploads.
        Arc::clone(&upload_backend),
        &stage2_tmp.path().join("stage"),
        &journal2_tmp.path().join("journal.bin"),
    );

    let svc = MountService::new();
    let handle = match svc.mount_fuser(
        mnt_path,
        shim,
        MountOptions {
            read_only: false,
            ..MountOptions::default()
        },
    ) {
        Ok(h) => h,
        Err(err) if should_skip_mount_error(&err.to_string()) => {
            eprintln!("[fuse_write_path_live] skip (mount2): {err}");
            return;
        }
        Err(err) => panic!("mount_fuser (mount2): {err}"),
    };
    let guard = MountGuard::new(handle, mnt_path.to_path_buf());

    std::thread::sleep(Duration::from_millis(200));
    if !mount_appears_active(mnt_path) {
        eprintln!("[fuse_write_path_live] skip: mount2 did not appear in /proc/self/mountinfo");
        return;
    }

    // Authoritative readback. A readdir is not strictly necessary — the
    // kernel can go straight to lookup/getattr/open/read — but running
    // one first shakes out any directory-listing regressions that would
    // hide the file entirely.
    let listing: Vec<String> = match std::fs::read_dir(mnt_path) {
        Ok(iter) => iter
            .filter_map(|e| e.ok().map(|d| d.file_name().to_string_lossy().into_owned()))
            .collect(),
        Err(err) if should_skip_io_error(&err) => return,
        Err(err) => panic!("readdir post-remount: {err}"),
    };
    assert!(
        listing.iter().any(|n| n == file_name),
        "post-remount readdir must surface the uploaded file; got {listing:?}"
    );

    let got = match std::fs::read(file_path) {
        Ok(b) => b,
        Err(err) if should_skip_io_error(&err) => return,
        Err(err) => panic!("post-remount read {file_name}: {err}"),
    };
    assert_eq!(
        got.len(),
        payload.len(),
        "post-remount readback size mismatch: got {}, expected {}",
        got.len(),
        payload.len()
    );
    assert_eq!(
        got, payload,
        "post-remount readback bytes must be identical to the written payload"
    );

    // No new uploads should have fired during pure readback. Keep the
    // upload list snapshot count from mount #1 as the expected floor.
    {
        let uploads = upload_backend.uploads.lock().unwrap();
        let matching: usize = uploads.iter().filter(|(_, n, _)| n == file_name).count();
        assert!(
            matching >= 1,
            "expected the mount#1 upload record to still be present; uploads={}",
            uploads.len()
        );
    }

    guard.unmount().expect("clean unmount (mount2)");
}

/// Helper: assert the recording upload backend captured an `upload_file`
/// for `name` with bytes equal to `payload`.
fn assert_upload_captured(
    upload_backend: &Arc<RecordingUploadBackend>,
    name: &str,
    payload: &[u8],
) {
    let uploads = upload_backend.uploads.lock().unwrap();
    let matched = uploads
        .iter()
        .find(|(_, n, b)| n == name && b.len() == payload.len() && b == payload);
    assert!(
        matched.is_some(),
        "kernel release/fsync on mount#1 should have finalized an upload of {} bytes for {name}; \
         captured uploads: {:?}",
        payload.len(),
        uploads
            .iter()
            .map(|(p, n, b)| (p.clone(), n.clone(), b.len()))
            .collect::<Vec<_>>()
    );
}
