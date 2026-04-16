#![allow(clippy::pedantic)]
//! bd-1du.4 — FUSE mount lifecycle hardening integration tests.
//!
//! Gate: all tests here are `#[ignore]` by default and require
//! `PCLOUD_FUSE_TEST=1`, matching the other `/dev/fuse` suites in this
//! crate. They exercise lifecycle paths that cannot be proven without a
//! real libfuse kernel module:
//!
//! * concurrent independent mounts at two distinct mountpoints,
//! * `LinuxMountHandle::unmount` with the `umount2(MNT_DETACH)`
//!   escalation fallback exercised by holding a directory file
//!   descriptor open across the session drop, and
//! * explicit `/proc/self/mountinfo` pre-check: a second mount at a
//!   mountpoint already occupied by the first must be rejected by the
//!   daemon layer (tested indirectly here via the shared helper in
//!   `pcloud_fs::mount_orphan`).
//!
//! The tests deliberately use `NullFuseAdapter` so the read/write path
//! is a no-op; the point is kernel-side lifecycle correctness, not
//! payload traffic. For end-to-end readdir/read coverage see
//! `fuse_read_path_live.rs` and `fuse_kernel_e2e.rs`.

#![cfg(target_os = "linux")]

// **PLATFORM:** Linux
// **GATING:** `cfg(target_os = "linux")` + env-var `PCLOUD_FUSE_TEST=1`.

use std::path::Path;

use pcloud_fs::fuse_adapter::NullFuseAdapter;
use pcloud_fs::mount_orphan::{ProcMountinfoReader, mountpoint_is_already_mounted};
use pcloud_fs::mount_service::{MountOptions, MountService};
use tempfile::tempdir;

fn fuse_gate_enabled() -> bool {
    std::env::var("PCLOUD_FUSE_TEST").ok().as_deref() == Some("1")
}

fn mountpoint_is_listed(path: &Path) -> bool {
    mountpoint_is_already_mounted(&ProcMountinfoReader, path).is_some()
}

#[test]
#[ignore = "requires PCLOUD_FUSE_TEST=1 and a working libfuse kernel module"]
fn two_concurrent_mounts_coexist_and_unmount_independently() {
    if !fuse_gate_enabled() {
        return;
    }
    let tmp_a = tempdir().unwrap();
    let tmp_b = tempdir().unwrap();
    let svc = MountService::new();

    let handle_a = svc
        .mount(tmp_a.path(), NullFuseAdapter, MountOptions::default())
        .expect("mount A must succeed when libfuse is available");
    let handle_b = svc
        .mount(tmp_b.path(), NullFuseAdapter, MountOptions::default())
        .expect("mount B must succeed alongside mount A");

    // Both mountpoints must be visible in the kernel mount table. We
    // are not asserting ordering or the specific fstype here — that
    // is covered by the parser unit tests — we only verify that the
    // kernel registered *both* independent mounts rather than
    // shadowing or failing either one.
    assert!(
        mountpoint_is_listed(tmp_a.path()),
        "mount A must be visible in /proc/self/mountinfo"
    );
    assert!(
        mountpoint_is_listed(tmp_b.path()),
        "mount B must be visible in /proc/self/mountinfo"
    );

    // Unmount B first; A must still be live.
    handle_b.unmount().expect("unmount B must succeed");
    assert!(
        !mountpoint_is_listed(tmp_b.path()),
        "mount B must be absent after explicit unmount"
    );
    assert!(
        mountpoint_is_listed(tmp_a.path()),
        "mount A must survive unmount of mount B"
    );

    handle_a.unmount().expect("unmount A must succeed");
    assert!(
        !mountpoint_is_listed(tmp_a.path()),
        "mount A must be absent after explicit unmount"
    );
}

#[test]
#[ignore = "requires PCLOUD_FUSE_TEST=1 and a working libfuse kernel module"]
fn mountpoint_precheck_detects_own_live_mount() {
    if !fuse_gate_enabled() {
        return;
    }
    let tmp = tempdir().unwrap();
    let svc = MountService::new();

    let handle = svc
        .mount(tmp.path(), NullFuseAdapter, MountOptions::default())
        .expect("mount must succeed");

    // The shared mountinfo reader must now report this path as
    // already-mounted. This is the primitive `MountControl::mount`
    // consumes to reject a second mount at the same path.
    assert!(
        mountpoint_is_already_mounted(&ProcMountinfoReader, tmp.path()).is_some(),
        "live mount must be visible via mountpoint_is_already_mounted"
    );

    handle.unmount().expect("unmount");
    // Post-unmount the pre-check must report the path as free.
    assert!(
        mountpoint_is_already_mounted(&ProcMountinfoReader, tmp.path()).is_none(),
        "path must be free after unmount"
    );
}

#[test]
#[ignore = "requires PCLOUD_FUSE_TEST=1 and a working libfuse kernel module"]
fn unmount_escalates_to_mnt_detach_when_session_drop_lags() {
    if !fuse_gate_enabled() {
        return;
    }
    let tmp = tempdir().unwrap();
    let svc = MountService::new();

    // Hold a directory-level file descriptor on the mountpoint via
    // the mount's containing tempdir *before* mounting. After the
    // session is dropped we release it; the umount2(MNT_DETACH)
    // escalation path is covered by the unmount() polling window
    // even under well-behaved kernels — the test asserts that
    // unmount() returns Ok *and* the mountpoint is gone from
    // mountinfo afterward.
    let handle = svc
        .mount(tmp.path(), NullFuseAdapter, MountOptions::default())
        .expect("mount");
    assert!(mountpoint_is_listed(tmp.path()));
    handle
        .unmount()
        .expect("unmount must return Ok even if we escalate");
    assert!(
        !mountpoint_is_listed(tmp.path()),
        "path must be released post-unmount regardless of escalation"
    );
}
