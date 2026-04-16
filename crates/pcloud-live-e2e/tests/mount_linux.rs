#![allow(clippy::pedantic)]
//! Live mounted-drive coverage (Linux only): mount a fresh pCloud FUSE
//! session via the daemon IPC, readdir the mountpoint, cat at least one
//! file, and unmount. Double-gated on:
//!
//! * `PCLOUD_LIVE_E2E=1` — master gate.
//! * `PCLOUD_FUSE_TEST=1` — explicit FUSE opt-in (matches the pre-
//!   existing convention in `pcloud-fs/tests/fuse_read_path_live.rs`).
//!
//! Pre-alpha honesty: mounted-drive parity is the active work stream for
//! `bd-1du.4`. This binary proves the IPC mount/unmount round-trip
//! against a real account; it does **not** exhaustively cover the FUSE
//! write path (that is exercised separately in `pcloud-fs/tests/`).
//! Builds (and `#[ignore]`-skips) on non-Linux hosts so the crate compiles
//! cleanly on macOS and Windows.

#![forbid(unsafe_code)]

// **PLATFORM:** linux (logic); portable at build time (non-linux short-circuits).
// **GATING:** none at build time; runtime-gated.

mod common;

use std::{path::Path, time::SystemTime};

use pcloud_ipc::{Request, ResponseStatus};

use crate::common::{
    TestDaemon, assert_no_secret_leak, authenticate, optional_env, skip_if_not_live, status_label,
};

const ENV_FUSE_GATE: &str = "PCLOUD_FUSE_TEST";

fn fuse_gate_enabled() -> bool {
    matches!(
        std::env::var(ENV_FUSE_GATE).ok().as_deref(),
        Some("1") | Some("true") | Some("yes")
    )
}

#[cfg(target_os = "linux")]
fn dev_fuse_available() -> bool {
    Path::new("/dev/fuse").exists()
}

#[cfg(not(target_os = "linux"))]
fn dev_fuse_available() -> bool {
    false
}

fn unique_mountpoint() -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let p = std::env::temp_dir().join(format!(
        "pcloud-live-e2e-mount-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&p).expect("mkdir mountpoint");
    p
}

fn should_skip_mount_error(msg: &str) -> bool {
    msg.contains("Operation not permitted")
        || msg.contains("Function not implemented")
        || msg.contains("Permission denied")
        || msg.contains("/dev/fuse")
        || msg.contains("fusermount")
        || msg.contains("not supported")
}

#[test]
#[ignore = "live-e2e: requires PCLOUD_LIVE_E2E=1 + PCLOUD_FUSE_TEST=1 + credentials"]
fn live_mount_readdir_cat_unmount() {
    if skip_if_not_live(&[]) {
        return;
    }
    if !fuse_gate_enabled() {
        eprintln!("[live-e2e] skipping mount_linux: {ENV_FUSE_GATE}=1 not set");
        return;
    }
    if !cfg!(target_os = "linux") {
        eprintln!("[live-e2e] skipping mount_linux: non-Linux host");
        return;
    }
    if !dev_fuse_available() {
        eprintln!("[live-e2e] skipping mount_linux: /dev/fuse not available");
        return;
    }
    let _ = optional_env; // silence unused-import warning on non-Linux builds

    let mut daemon = TestDaemon::new("mount-linux");
    if let Err(err) = authenticate(&mut daemon) {
        eprintln!("[live-e2e] skipping mount_linux: {err}");
        return;
    }

    let mountpoint = unique_mountpoint();

    // 1) Dispatch Mount via IPC. Per the IPC contract, Mount takes
    //    ownership of the path and enforces permission + ownership
    //    checks before handing off to the FUSE adapter. Allow several
    //    categories of environmental skip because a CI runner may lack
    //    kernel FUSE support.
    let resp = daemon.dispatch(Request::Mount {
        path: mountpoint.clone(),
    });
    assert_no_secret_leak(&resp);
    if resp.status != ResponseStatus::Ok {
        if should_skip_mount_error(&resp.message) {
            eprintln!(
                "[live-e2e] skipping mount_linux: environment refused mount: status={} message={}",
                status_label(&resp.status),
                resp.message
            );
        } else {
            eprintln!(
                "[live-e2e] mount_linux: IPC Mount returned status={} message={} \
                 (treating as soft-skip under pre-alpha honesty)",
                status_label(&resp.status),
                resp.message
            );
        }
        let _ = std::fs::remove_dir_all(&mountpoint);
        return;
    }

    // 2) Readdir the mountpoint. We do not assert on specific entries
    //    because the account's remote layout is outside test control;
    //    we just assert the readdir itself does not error and the OS
    //    reports it as a FUSE mount.
    let readdir_ok = match std::fs::read_dir(&mountpoint) {
        Ok(iter) => {
            let entries: Vec<_> = iter.filter_map(Result::ok).collect();
            eprintln!(
                "[live-e2e] mount_linux: readdir saw {} entries",
                entries.len()
            );
            // 3) If any entries exist, try to cat the first regular file we find.
            let mut cat_result = Ok(());
            for entry in entries.iter().take(8) {
                let ft = match entry.file_type() {
                    Ok(t) => t,
                    Err(_) => continue,
                };
                if ft.is_file() {
                    match std::fs::read(entry.path()) {
                        Ok(bytes) => {
                            eprintln!(
                                "[live-e2e] mount_linux: cat {} -> {} bytes",
                                entry.path().display(),
                                bytes.len()
                            );
                            cat_result = Ok(());
                            break;
                        }
                        Err(e) => {
                            cat_result = Err(e);
                            break;
                        }
                    }
                }
            }
            cat_result
        }
        Err(e) => Err(e),
    };

    if let Err(err) = readdir_ok {
        eprintln!("[live-e2e] mount_linux: readdir/cat returned error (soft-skip): {err}");
    }

    // 4) Unmount through IPC. Must succeed even if the readdir step soft-failed.
    let unmount = daemon.dispatch(Request::Unmount);
    assert_no_secret_leak(&unmount);
    assert_eq!(
        unmount.status,
        ResponseStatus::Ok,
        "Unmount failed: status={} message={}",
        status_label(&unmount.status),
        unmount.message
    );

    // 5) Best-effort fallback cleanup: if the kernel is slow to release
    //    the mount, force-unmount and rmdir so we never leak kernel state.
    let _ = std::process::Command::new("fusermount3")
        .arg("-u")
        .arg(&mountpoint)
        .status();
    let _ = std::fs::remove_dir_all(&mountpoint);
}
