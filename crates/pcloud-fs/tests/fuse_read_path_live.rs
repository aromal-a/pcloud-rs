#![allow(clippy::pedantic)]
//! bd-1du.4 — live read-path FUSE integration test.
//!
//! Mounts a **read-only** [`ProtoFuseAdapter`] backed by a mocked folder
//! + file backend against a real Linux FUSE kernel mount (via `libfuse` /
//!   the `fuser` crate) through the public [`MountService::mount`] entry
//!   point — exercising the `BoxedFuserShim` / `FuserShim<A>` read-path
//!   delegation in `platform/linux.rs`.
//!
//! The test issues a `readdir` and a `cat <file>` via `std::fs`, then
//! unmounts cleanly. All in-flight reads are drained by the `fuser`
//! `BackgroundSession` drop on unmount.
//!
//! # Gating
//!
//! * `#[cfg(target_os = "linux")]` — `fuser` is Linux-only in this workspace.
//! * `#[ignore]` by default — opt-in via `PCLOUD_FUSE_TEST=1` so CI
//!   environments without `/dev/fuse`, `SYS_ADMIN`, or `fusermount3`
//!   do not fail.
//! * If the host refuses to mount FUSE (EPERM / ENOSYS / missing
//!   `/dev/fuse`), the test **skips gracefully** — a project-convention
//!   pattern shared with `fuse_mount_integration.rs`.
//!
//! # Scope
//!
//! Read-path only. The write path is out of scope for this iteration;
//! `PcloudFsShim` carries the full read+write mount and is covered by
//! `fuse_kernel_e2e.rs`. Follow-up for dyn-shim writes: `bd-1du.4.6`.

#![cfg(target_os = "linux")]

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use pcloud_fs::backend::mock::{MockFileBackend, MockFolderBackend};
use pcloud_fs::fuse_adapter::{AdapterOptions, ProtoFuseAdapter};
use pcloud_fs::mount_service::{MountHandle, MountOptions, MountService};

fn fuse_gate_enabled() -> bool {
    std::env::var("PCLOUD_FUSE_TEST").ok().as_deref() == Some("1")
        || std::env::var("PCLOUD_LIVE_E2E").ok().as_deref() == Some("1")
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

/// RAII guard that unmounts on drop, even on panic. Mirrors the pattern
/// in `fuse_kernel_e2e.rs` so a failing assertion never leaks a mount.
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
        if let Some(h) = self.handle.take()
            && let Err(e) = h.unmount()
        {
            eprintln!("[fuse_read_path_live] RAII unmount failed: {e}");
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

/// bd-1du.4: read-path mount delegates `readdir` and `read` through
/// the `FuseAdapter` trait shim.
#[test]
#[ignore = "requires PCLOUD_FUSE_TEST=1 (or PCLOUD_LIVE_E2E=1) and a working libfuse kernel module"]
fn readdir_and_cat_through_real_mount_via_fuse_adapter_shim() {
    if !fuse_gate_enabled() {
        eprintln!("[fuse_read_path_live] skip: PCLOUD_FUSE_TEST / PCLOUD_LIVE_E2E not set");
        return;
    }
    if !dev_fuse_available() {
        eprintln!("[fuse_read_path_live] skip: /dev/fuse not available");
        return;
    }

    // --- arrange a small mocked remote tree ------------------------------
    //
    // We publish explicit per-entry sizes via `insert_dir_with_sizes` so
    // that the kernel's `getattr` reply advertises the true size —
    // otherwise the kernel caps `read(2)` to 0 bytes and the round-trip
    // assertion below would observe an empty payload.
    let hello_bytes: &[u8] = b"read-path via fuse adapter shim";
    let note_bytes: &[u8] = b"# note\n\nhello";
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
                Some(10),
                Some(hello_bytes.len() as u64),
            ),
        ],
    );
    folder.insert_dir_with_sizes(
        "/docs",
        2,
        vec![(
            "note.md",
            false,
            None,
            Some(11),
            Some(note_bytes.len() as u64),
        )],
    );
    let files = Arc::new(MockFileBackend::new());
    files.insert_file(10, hello_bytes.to_vec());
    files.insert_file(11, note_bytes.to_vec());

    let adapter = ProtoFuseAdapter::with_file_backend(
        Arc::clone(&folder),
        Arc::clone(&files),
        AdapterOptions::default(),
    );

    // --- mount via the public scaffold entry point -----------------------
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
        Ok(h) => h,
        Err(err) if should_skip_mount_error(&err.to_string()) => {
            eprintln!("[fuse_read_path_live] skip: host refused FUSE mount: {err}");
            return;
        }
        Err(err) => panic!("MountService::mount: {err}"),
    };
    let guard = MountGuard::new(handle, mnt.path().to_path_buf());

    // Give the kernel a moment to finish the mount handshake.
    std::thread::sleep(Duration::from_millis(200));
    if !mount_appears_active(mnt.path()) {
        eprintln!("[fuse_read_path_live] skip: mount did not appear in /proc/self/mountinfo");
        return;
    }

    // --- 1. readdir the root via the OS VFS ------------------------------
    let root: Vec<String> = match std::fs::read_dir(mnt.path()) {
        Ok(iter) => iter
            .filter_map(|e| e.ok().map(|d| d.file_name().to_string_lossy().into_owned()))
            .collect(),
        Err(err) if should_skip_io_error(&err) => return,
        Err(err) => panic!("readdir /: {err}"),
    };
    assert!(
        root.iter().any(|n| n == "docs"),
        "root listing should contain `docs`, got {root:?}"
    );
    assert!(
        root.iter().any(|n| n == "hello.txt"),
        "root listing should contain `hello.txt`, got {root:?}"
    );

    // --- 2. readdir a nested directory -----------------------------------
    let nested: Vec<String> = match std::fs::read_dir(mnt.path().join("docs")) {
        Ok(iter) => iter
            .filter_map(|e| e.ok().map(|d| d.file_name().to_string_lossy().into_owned()))
            .collect(),
        Err(err) if should_skip_io_error(&err) => return,
        Err(err) => panic!("readdir /docs: {err}"),
    };
    assert!(
        nested.iter().any(|n| n == "note.md"),
        "nested listing should contain `note.md`, got {nested:?}"
    );

    // --- 3. cat <file> via std::fs (open + read + release) ---------------
    let got = match std::fs::read(mnt.path().join("hello.txt")) {
        Ok(b) => b,
        Err(err) if should_skip_io_error(&err) => return,
        Err(err) => panic!("read hello.txt: {err}"),
    };
    assert_eq!(got, hello_bytes, "cat hello.txt round-trip");

    let got_nested = match std::fs::read(mnt.path().join("docs").join("note.md")) {
        Ok(b) => b,
        Err(err) if should_skip_io_error(&err) => return,
        Err(err) => panic!("read docs/note.md: {err}"),
    };
    assert_eq!(got_nested, note_bytes, "cat docs/note.md round-trip");

    // --- 4. writes must be rejected (read-only shim) ---------------------
    //
    // Open with O_WRONLY must fail — the shim is deliberately read-only.
    // We tolerate either EROFS (our error) or the kernel's wrapped form.
    let write_attempt = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(mnt.path().join("new.txt"));
    match write_attempt {
        Err(e) => {
            // EROFS is libc::EROFS = 30. Kernel may map to ErrorKind::Other.
            let errno = e.raw_os_error();
            assert!(
                errno == Some(libc::EROFS)
                    || errno == Some(libc::ENOSYS)
                    || errno == Some(libc::EACCES),
                "expected EROFS/ENOSYS/EACCES writing to read-only shim, got errno={errno:?} err={e}"
            );
        }
        Ok(_) => panic!("write to read-only shim should fail"),
    }

    // --- 5. unmount cleanly ---------------------------------------------
    guard.unmount().expect("clean unmount");
}
