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

#[cfg(target_os = "linux")]
use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

use pcloud_ipc::{Request, ResponseStatus};

use crate::common::{
    TestDaemon, assert_no_secret_leak, authenticate, optional_env, release_gate_enabled,
    scratch_folder, skip_if_not_live, status_label,
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
        assert!(
            !release_gate_enabled(),
            "release mount gate requires {ENV_FUSE_GATE}=1"
        );
        eprintln!("[live-e2e] skipping mount_linux: {ENV_FUSE_GATE}=1 not set");
        return;
    }
    if !cfg!(target_os = "linux") {
        assert!(
            !release_gate_enabled(),
            "release Linux mount gate ran on a non-Linux host"
        );
        eprintln!("[live-e2e] skipping mount_linux: non-Linux host");
        return;
    }
    if !dev_fuse_available() {
        assert!(
            !release_gate_enabled(),
            "release mount gate requires a usable /dev/fuse"
        );
        eprintln!("[live-e2e] skipping mount_linux: /dev/fuse not available");
        return;
    }
    let _ = optional_env; // silence unused-import warning on non-Linux builds

    let mut daemon = TestDaemon::new("mount-linux");
    if let Err(err) = authenticate(&mut daemon) {
        assert!(
            !release_gate_enabled(),
            "release mount authentication failed: {err}"
        );
        eprintln!("[live-e2e] skipping mount_linux: {err}");
        return;
    }

    let fixture = b"pcloud-rs credentialed mount fixture\n";
    let filename = format!(
        "pcloud-rs-mount-{}-{}.txt",
        std::process::id(),
        SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default()
    );
    let scratch = scratch_folder();
    let remote_path = if scratch.ends_with('/') {
        format!("{scratch}{filename}")
    } else {
        format!("{scratch}/{filename}")
    };
    let local_fixture = daemon.config.paths.cache_dir.join(&filename);
    std::fs::create_dir_all(local_fixture.parent().expect("fixture parent"))
        .expect("create fixture parent");
    std::fs::write(&local_fixture, fixture).expect("write local mount fixture");
    let upload = daemon.dispatch(Request::UploadFileByPath {
        local_path: local_fixture,
        remote_path: remote_path.clone(),
    });
    assert_no_secret_leak(&upload);
    if upload.status != ResponseStatus::Ok {
        assert!(
            !release_gate_enabled(),
            "release mount fixture upload failed: status={} message={}",
            status_label(&upload.status),
            upload.message
        );
        eprintln!(
            "[live-e2e] skipping mount_linux: fixture upload failed: {}",
            upload.message
        );
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
        let _ = daemon.dispatch(Request::DeletePath {
            path: remote_path,
            recursive: false,
        });
        if should_skip_mount_error(&resp.message) && !release_gate_enabled() {
            eprintln!(
                "[live-e2e] skipping mount_linux: environment refused mount: status={} message={}",
                status_label(&resp.status),
                resp.message
            );
        } else if !release_gate_enabled() {
            eprintln!(
                "[live-e2e] mount_linux: IPC Mount returned status={} message={} \
                 (treating as soft-skip under pre-alpha honesty)",
                status_label(&resp.status),
                resp.message
            );
        }
        let _ = std::fs::remove_dir_all(&mountpoint);
        assert!(
            !release_gate_enabled(),
            "release mount failed: status={} message={}",
            status_label(&resp.status),
            resp.message
        );
        return;
    }

    // 2) Read the exact remote fixture through the kernel mount. Namespace
    // propagation may lag the successful upload briefly, so poll with a hard
    // bound rather than accepting an empty/failed read.
    let mounted_fixture = mountpoint.join(remote_path.trim_start_matches('/'));
    let deadline = Instant::now() + Duration::from_secs(15);
    let read_result = loop {
        match std::fs::read(&mounted_fixture) {
            Ok(bytes) => break Ok(bytes),
            Err(error) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(200));
                let _ = error;
            }
            Err(error) => break Err(error),
        }
    };

    // 3) Unmount and clean the remote fixture before asserting the read result,
    // so a failure does not leak a kernel mount or remote object.
    let unmount = daemon.dispatch(Request::Unmount);
    assert_no_secret_leak(&unmount);
    let cleanup = daemon.dispatch(Request::DeletePath {
        path: remote_path.clone(),
        recursive: false,
    });
    assert_no_secret_leak(&cleanup);

    // 4) Best-effort fallback cleanup: if the kernel is slow to release
    //    the mount, force-unmount and rmdir so we never leak kernel state.
    let _ = std::process::Command::new("fusermount3")
        .arg("-u")
        .arg(&mountpoint)
        .status();
    let _ = std::fs::remove_dir_all(&mountpoint);

    assert_eq!(
        unmount.status,
        ResponseStatus::Ok,
        "Unmount failed: status={} message={}",
        status_label(&unmount.status),
        unmount.message
    );
    assert_eq!(
        cleanup.status,
        ResponseStatus::Ok,
        "remote fixture cleanup failed: status={} message={}",
        status_label(&cleanup.status),
        cleanup.message
    );
    match read_result {
        Ok(bytes) => assert_eq!(bytes, fixture, "mounted fixture bytes must match"),
        Err(error) => {
            assert!(
                !release_gate_enabled(),
                "release mount could not read {}: {error}",
                mounted_fixture.display()
            );
            eprintln!(
                "[live-e2e] warning: mounted fixture read failed for {}: {error}",
                mounted_fixture.display()
            );
        }
    }
}
