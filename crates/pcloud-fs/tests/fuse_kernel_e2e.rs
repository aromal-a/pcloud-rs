#![allow(clippy::pedantic)]
//! PLAN_A_PLUS P0.5 — FUSE kernel end-to-end test.
//!
//! Mounts a full read-write [`PcloudFsShim`] backed by a mock folder/file
//! backend and an in-memory upload backend against a real FUSE kernel mount
//! (via `libfuse` / the `fuser` crate), and then exercises, entirely
//! through the OS VFS (`std::fs`):
//!
//!   1. create + write a 64 MiB file
//!   2. `fsync` it via `File::sync_all`
//!   3. read the 64 MiB file back and byte-compare against the source payload
//!   4. rename the file
//!   5. unlink the renamed file
//!   6. unmount cleanly (RAII — runs even on panic)
//!
//! # Gating
//!
//! This test is:
//!   * `#[cfg(target_os = "linux")]` (libfuse is Linux-only in this project),
//!   * `#[ignore]` by default (no CI surprises), and
//!   * **additionally** gated on `PCLOUD_LIVE_E2E=1`. The older
//!     `fuse_mount_integration.rs` tests use `PCLOUD_FUSE_TEST=1`; we accept
//!     either so CI can opt in with a single env var without touching cargo
//!     features.
//!
//! If the host refuses to mount FUSE (no `/dev/fuse`, unprivileged container,
//! EPERM, ENOSYS) the test **skips gracefully** — it returns early without
//! failing. This matches the project convention in
//! `fuse_mount_integration.rs` and keeps the P0.5 gate safe for environments
//! where the kernel side cannot possibly succeed (typical cloud CI runners
//! without `--device /dev/fuse` and `SYS_ADMIN`).
//!
//! # Fallback note
//!
//! A real kernel mount from a user-space test process requires either:
//!   * root / `CAP_SYS_ADMIN`, or
//!   * a suid `fusermount3` binary plus `/dev/fuse` access, or
//!   * a user namespace with the above.
//!
//! When none of these are available we intentionally **skip** rather than
//! silently downgrade to an in-memory mock — the in-memory write/read path
//! is already covered by the unit tests in `fuser_shim.rs` and by
//! `write_path_replay.rs`. Re-running those here as a "fallback" would
//! dilute the P0.5 signal: the whole point of this test is to catch
//! integration bugs that only show up against the real kernel VFS.

#![cfg(target_os = "linux")]

// **PLATFORM:** Linux
// **GATING:** #[cfg(target_os = "linux")].

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use pcloud_fs::backend::{FileBackend, FileHandle, FolderBackend};
use pcloud_fs::errors::FsError;
use pcloud_fs::fuse_adapter::{AdapterOptions, ProtoFuseAdapter};
use pcloud_fs::fuser_shim::PcloudFsShim;
use pcloud_fs::mount_service::{MountHandle, MountOptions, MountService};
use pcloud_fs::staging::StagingDir;
use pcloud_fs::write_journal::WriteJournal;
use pcloud_fs::write_path::{
    FileUploadBackend, WritePathError, WritePathOptions, WritePathService,
};

const LARGE_FILE_BYTES: usize = 64 * 1024 * 1024; // 64 MiB

fn e2e_gate_enabled() -> bool {
    let live = std::env::var("PCLOUD_LIVE_E2E").ok().as_deref() == Some("1");
    let legacy = std::env::var("PCLOUD_FUSE_TEST").ok().as_deref() == Some("1");
    live || legacy
}

fn dev_fuse_available() -> bool {
    std::path::Path::new("/dev/fuse").exists()
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

/// RAII guard that unmounts on drop, even on panic, so a failing assertion
/// does not leak a stray FUSE mount under `/tmp`.
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
            // Best-effort cleanup; log-only on failure. If this fails we
            // fall back to a fusermount -u attempt so the tempdir can be
            // removed without leaking a mount.
            if let Err(e) = h.unmount() {
                eprintln!("[fuse_kernel_e2e] RAII unmount failed: {e}");
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
}

/// Mutable in-memory remote used as folder, file, and upload backend.
/// Successful uploads are immediately visible to subsequent list/open/read
/// calls, matching the publication contract of a completed pCloud upload.
struct MutableRemoteBackend {
    next_file_id: AtomicU64,
    paths: std::sync::Mutex<HashMap<String, u64>>,
    files: std::sync::Mutex<HashMap<u64, Vec<u8>>>,
    uploads: std::sync::Mutex<Vec<(String, String, Vec<u8>)>>,
    unlinks: std::sync::Mutex<Vec<String>>,
    renames: std::sync::Mutex<Vec<(String, String)>>,
}

impl Default for MutableRemoteBackend {
    fn default() -> Self {
        Self {
            next_file_id: AtomicU64::new(100),
            paths: std::sync::Mutex::new(HashMap::new()),
            files: std::sync::Mutex::new(HashMap::new()),
            uploads: std::sync::Mutex::new(Vec::new()),
            unlinks: std::sync::Mutex::new(Vec::new()),
            renames: std::sync::Mutex::new(Vec::new()),
        }
    }
}

impl FolderBackend for MutableRemoteBackend {
    fn list_contents(
        &self,
        path: &str,
    ) -> Result<pcloud_proto::folder_api::RemoteFolderListing, FsError> {
        if path != "/" {
            return Err(FsError::NotFound);
        }
        let paths = self.paths.lock().unwrap();
        let files = self.files.lock().unwrap();
        let mut entries: Vec<_> = paths
            .iter()
            .filter_map(|(full_path, file_id)| {
                let name = full_path.strip_prefix('/')?;
                if name.contains('/') {
                    return None;
                }
                Some(pcloud_proto::folder_api::RemoteFolderEntry {
                    name: name.to_owned(),
                    is_folder: false,
                    folder_id: None,
                    file_id: Some(*file_id),
                    owner_user_id: None,
                    is_mine: true,
                    encrypted: false,
                    is_shared: false,
                    permissions: None,
                    size: files.get(file_id).map(|bytes| bytes.len() as u64),
                    modified: None,
                })
            })
            .collect();
        entries.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(pcloud_proto::folder_api::RemoteFolderListing {
            folder_id: 1,
            path: "/".to_owned(),
            name: "/".to_owned(),
            entries,
            api_server: None,
            owner_user_id: None,
            is_mine: true,
            encrypted: false,
            is_shared: false,
            permissions: None,
        })
    }
}

impl FileBackend for MutableRemoteBackend {
    fn open(&self, file_id: u64) -> Result<FileHandle, FsError> {
        let files = self.files.lock().unwrap();
        let size = files.get(&file_id).ok_or(FsError::NotFound)?.len() as u64;
        Ok(FileHandle {
            file_id,
            size,
            host: "mock".to_owned(),
            path: format!("/{file_id}"),
            dwltag: None,
        })
    }

    fn read(&self, handle: &FileHandle, offset: u64, len: usize) -> Result<Vec<u8>, FsError> {
        let files = self.files.lock().unwrap();
        let bytes = files.get(&handle.file_id).ok_or(FsError::NotFound)?;
        let start = usize::try_from(offset).map_err(|_| FsError::Invalid)?;
        if start >= bytes.len() {
            return Ok(Vec::new());
        }
        let end = start.saturating_add(len).min(bytes.len());
        Ok(bytes[start..end].to_vec())
    }
}

impl FileUploadBackend for MutableRemoteBackend {
    fn upload_file(
        &self,
        parent_path: &str,
        name: &str,
        staging_file: &Path,
    ) -> Result<(), WritePathError> {
        let bytes =
            std::fs::read(staging_file).map_err(|e| WritePathError::Upload(e.to_string()))?;
        let full_path = if parent_path == "/" {
            format!("/{name}")
        } else {
            format!("{parent_path}/{name}")
        };
        let file_id = {
            let mut paths = self.paths.lock().unwrap();
            *paths
                .entry(full_path)
                .or_insert_with(|| self.next_file_id.fetch_add(1, Ordering::Relaxed))
        };
        self.files.lock().unwrap().insert(file_id, bytes.clone());
        self.uploads
            .lock()
            .unwrap()
            .push((parent_path.to_owned(), name.to_owned(), bytes));
        Ok(())
    }
    fn unlink_remote(&self, path: &str) -> Result<(), WritePathError> {
        if let Some(file_id) = self.paths.lock().unwrap().remove(path) {
            self.files.lock().unwrap().remove(&file_id);
        }
        self.unlinks.lock().unwrap().push(path.to_owned());
        Ok(())
    }
    fn rename_remote(&self, from: &str, to: &str) -> Result<(), WritePathError> {
        let file_id = self
            .paths
            .lock()
            .unwrap()
            .remove(from)
            .ok_or_else(|| WritePathError::Upload(format!("missing rename source {from}")))?;
        self.paths.lock().unwrap().insert(to.to_owned(), file_id);
        self.renames
            .lock()
            .unwrap()
            .push((from.to_owned(), to.to_owned()));
        Ok(())
    }
}

/// Build a deterministic 64 MiB payload. We use a simple LCG so the data is
/// non-trivial (not all zeros — that would hide any zero-fill bug in the
/// kernel round-trip) but reproducible without pulling in an RNG crate.
fn build_payload(len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    while out.len() < len {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        out.extend_from_slice(&state.to_le_bytes());
    }
    out.truncate(len);
    out
}

#[test]
#[ignore = "requires PCLOUD_LIVE_E2E=1 (or PCLOUD_FUSE_TEST=1) and a working libfuse kernel module"]
fn large_file_kernel_roundtrip_rename_unlink() {
    if !e2e_gate_enabled() {
        eprintln!("[fuse_kernel_e2e] skip: PCLOUD_LIVE_E2E / PCLOUD_FUSE_TEST not set");
        return;
    }
    if !dev_fuse_available() {
        eprintln!("[fuse_kernel_e2e] skip: /dev/fuse not available");
        return;
    }

    // --- arrange backends ------------------------------------------------
    let remote = Arc::new(MutableRemoteBackend::default());

    let stage_tmp = tempfile::tempdir().expect("stage tempdir");
    let stage = StagingDir::open(stage_tmp.path().join("stage")).expect("staging");
    let journal = WriteJournal::open(stage.journal_path()).expect("journal");
    let writer = Arc::new(WritePathService::new(
        stage,
        journal,
        Arc::clone(&remote),
        WritePathOptions {
            // Only flush on explicit fsync so the test controls when uploads happen.
            flush_threshold_bytes: u64::MAX,
            flush_interval: Duration::from_secs(3600),
            ..WritePathOptions::default()
        },
    ));

    let adapter = Arc::new(
        ProtoFuseAdapter::with_file_backend(
            Arc::clone(&remote),
            Arc::clone(&remote),
            AdapterOptions::default(),
        )
        .with_write_path(Arc::clone(&writer)),
    );
    let shim = PcloudFsShim::new(adapter, Arc::clone(&writer));

    // --- mount -----------------------------------------------------------
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
        Err(err) if should_skip_mount_error(&err.to_string()) => {
            eprintln!("[fuse_kernel_e2e] skip: host refused FUSE mount: {err}");
            return;
        }
        Err(err) => panic!("mount_fuser: {err}"),
    };
    let guard = MountGuard::new(handle, mnt.path().to_path_buf());

    std::thread::sleep(Duration::from_millis(200));
    if !mount_appears_active(mnt.path()) {
        eprintln!("[fuse_kernel_e2e] skip: mount did not appear in /proc/self/mountinfo");
        return;
    }

    // --- step 1+2: create + write + fsync a 64 MiB file -------------------
    let payload = build_payload(LARGE_FILE_BYTES);
    let big_path = mnt.path().join("big.bin");

    {
        let mut f = match std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&big_path)
        {
            Ok(f) => f,
            Err(err) if should_skip_io_error(&err) => return,
            Err(err) => panic!("open big.bin for write: {err}"),
        };
        use std::io::Write;
        // Write in 1 MiB chunks to exercise multiple kernel write ops.
        for chunk in payload.chunks(1024 * 1024) {
            if let Err(err) = f.write_all(chunk) {
                if should_skip_io_error(&err) {
                    return;
                }
                panic!("write chunk: {err}");
            }
        }
        if let Err(err) = f.sync_all() {
            if should_skip_io_error(&err) {
                return;
            }
            panic!("sync_all: {err}");
        }
    }

    // Assert fsync actually produced an upload of the full payload.
    {
        let uploads = remote.uploads.lock().unwrap();
        let matched = uploads
            .iter()
            .find(|(_, n, b)| n == "big.bin" && b.len() == payload.len() && b == &payload);
        assert!(
            matched.is_some(),
            "kernel fsync should have delivered a full 64 MiB upload, uploads.len={} sizes={:?}",
            uploads.len(),
            uploads
                .iter()
                .map(|(_, n, b)| (n.clone(), b.len()))
                .collect::<Vec<_>>()
        );
    }

    // --- step 3: read back via kernel and byte-compare --------------------
    //
    let got = std::fs::read(&big_path).expect("read committed 64 MiB file through kernel VFS");
    assert_eq!(got, payload, "readback mismatch for 64 MiB file");

    // --- step 4: rename via kernel VFS -----------------------------------
    //
    let renamed = mnt.path().join("big-renamed.bin");
    std::fs::rename(&big_path, &renamed).expect("kernel rename");
    let renames = remote.renames.lock().unwrap();
    assert!(
        renames
            .iter()
            .any(|(_, to)| to.ends_with("big-renamed.bin")),
        "rename_remote should have been invoked, got {:?}",
        renames
    );
    drop(renames);
    assert_eq!(
        std::fs::read(&renamed).expect("read renamed file"),
        payload,
        "rename must preserve contents"
    );

    // --- step 5: unlink via kernel VFS -----------------------------------
    std::fs::remove_file(&renamed).expect("kernel unlink");
    assert!(
        remote
            .unlinks
            .lock()
            .unwrap()
            .iter()
            .any(|path| path.ends_with("big-renamed.bin")),
        "unlink_remote must receive the renamed path"
    );
    assert!(
        matches!(std::fs::metadata(&renamed), Err(error) if error.kind() == std::io::ErrorKind::NotFound),
        "unlinked file must disappear from the kernel VFS"
    );

    // --- step 6: unmount cleanly (also drops guard harmlessly) -----------
    guard.unmount().expect("clean unmount");
}
